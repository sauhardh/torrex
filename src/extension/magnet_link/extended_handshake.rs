use hex;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_bencode::value::Value;

use std::borrow::Cow;
use std::fmt::Write;

use crate::{
    bencode::Bencode, extension::magnet_link::Parser, handshake::Handshake, peers::ResponseParams,
    utils,
};

#[derive(Serialize, Deserialize, Debug)]
pub struct MagnetPeerInfo {
    #[serde(rename = "peer id")]
    peer_id: String,
    ip: String,
    port: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MagnetResponseParams {
    interval: u8,
    #[serde(rename = "min interval")]
    min_interval: u8,
    peers: Vec<MagnetPeerInfo>,
}

#[derive(Debug)]
pub struct ExtendedExchange<'a> {
    peers: Vec<String>,
    handshake: Handshake,
    pub parser: &'a mut Parser,
}

impl<'a> ExtendedExchange<'a> {
    pub fn new(parser: &'a mut Parser) -> Self {
        Self {
            peers: Vec::new(),
            handshake: Handshake::default(),
            parser,
        }
    }

    pub async fn get_peers(&mut self, peer_id: String) {
        let client = reqwest::Client::new();
        let mut url = Url::parse(&self.parser.magnet_link.tr.as_mut().unwrap().to_string())
            .expect("Could not parse provided url");

        let info_hash =
            hex::decode(&self.parser.magnet_link.xt).expect("Could not decode provided info_hash");

        url.query_pairs_mut()
            .encoding_override(Some(&|input| {
                if input == "{{info_hash}}" {
                    Cow::Owned(info_hash.clone())
                } else {
                    Cow::Borrowed(input.as_bytes())
                }
            }))
            .append_pair("info_hash", "{{info_hash}}")
            .append_pair("peer_id", &peer_id)
            .append_pair("left", "999")
            .append_pair("downloaded", "0")
            .append_pair("uploaded", "0")
            .append_pair("port", "6881");

        if let Ok(res) = client.get(url).send().await {
            if let Ok(buf) = res.bytes().await {
                let response: MagnetResponseParams =
                    serde_json::from_value(Bencode::new().decoder(&buf).0).unwrap();
                println!("response {:?}", response);
                //res: "d8:intervali60e12:min intervali60e5:peersld7:peer id20:-RN0.0.0-�\u{1c}��ۆ�jʚ!2:ip13:167.71.143.544:porti51501eed7:peer id20:-RN0.0.0-\u{12}\u{8}F�\u{10}% �%��2:ip14:139.59.184.2554:porti51510eed2:ip14:165.232.35.1394:porti51413e7:peer id20:-RN0.0.0-2��Qᓮ��ɍee8:completei3e10:incompletei1ee"
            }
        };
    }
}

#[cfg(test)]
mod test_extdhandshake {

    use crate::extension::magnet_link::Parser;

    use super::*;
    use utils::random;

    #[tokio::test]
    async fn test() {
        let url: String = "magnet:?xt=urn:btih:ad42ce8109f54c99613ce38f9b4d87e70f24a165&dn=magnet1.gif&tr=http%3A%2F%2Fbittorrent-test-tracker.codecrafters.io%2Fannounce".to_string();
        let mut parser = Parser::new(url.clone());

        let peer_id = random::generate_peerid();
        ExtendedExchange::new(parser.parse())
            .get_peers(peer_id)
            .await;
    }
}
